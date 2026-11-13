//! Identifier highlighting layered over the `TextMate` grammar. Scope
//! resolution handles values; the cached AST distinguishes types and table
//! keys from superficially identical value expressions.

use alloc::collections::BTreeMap;

use luck_ast::{
    Call, ChainHead, Expression, Field, GenericTypeList, Statement, Suffix, TableConstructor, Type,
    TypeDeclarationValue, TypeField, Visitor,
};
use luck_semantic::{StdlibEntry, SymbolKind};
use luck_token::{Span, Token, TokenKind, index_to_offset, offset_to_index};
use rustc_hash::FxHashMap;
use tower_lsp_server::ls_types::{
    Position, Range, SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens,
    SemanticTokensLegend, SemanticTokensRangeResult, SemanticTokensResult,
};

use crate::document::DocumentState;
use crate::providers::cursor::dotted_chain_ending_tokens;

const TOKEN_TYPES: &[SemanticTokenType] = &[
    SemanticTokenType::NAMESPACE,
    SemanticTokenType::FUNCTION,
    SemanticTokenType::PROPERTY,
    SemanticTokenType::VARIABLE,
    SemanticTokenType::PARAMETER,
    SemanticTokenType::METHOD,
    SemanticTokenType::TYPE,
];

// Indexes into `TOKEN_TYPES`.
const TYPE_NAMESPACE: u32 = 0;
const TYPE_FUNCTION: u32 = 1;
const TYPE_PROPERTY: u32 = 2;
const TYPE_VARIABLE: u32 = 3;
const TYPE_PARAMETER: u32 = 4;
const TYPE_METHOD: u32 = 5;
const TYPE_TYPE: u32 = 6;

const TOKEN_MODIFIERS: &[SemanticTokenModifier] = &[
    SemanticTokenModifier::DEPRECATED,
    SemanticTokenModifier::READONLY,
    SemanticTokenModifier::DEFAULT_LIBRARY,
];

// Bits for `TOKEN_MODIFIERS`, in order.
const MODIFIER_DEPRECATED: u32 = 1 << 0;
const MODIFIER_READONLY: u32 = 1 << 1;
const MODIFIER_DEFAULT_LIBRARY: u32 = 1 << 2;

#[derive(Debug, Clone, Copy)]
struct Classification {
    token_type: u32,
    modifiers: u32,
}

impl Classification {
    const fn plain(token_type: u32) -> Self {
        Self { token_type, modifiers: 0 }
    }
}

/// Classified tokens keyed by start offset. A later pass over the same
/// offset overwrites an earlier one.
type RawTokens = BTreeMap<u32, (Span, Classification)>;

pub(crate) fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: TOKEN_TYPES.to_vec(),
        token_modifiers: TOKEN_MODIFIERS.to_vec(),
    }
}

pub(crate) fn semantic_tokens(document: &DocumentState) -> SemanticTokensResult {
    SemanticTokensResult::Tokens(encode_tokens(document, None))
}

/// Deltas are re-encoded from the filtered list, so the first token is
/// relative to the document start exactly as the protocol requires.
pub(crate) fn in_range(document: &DocumentState, range: Range) -> SemanticTokensRangeResult {
    let byte_range = Span::new(document.offset(range.start), document.offset(range.end));
    SemanticTokensRangeResult::Tokens(encode_tokens(document, Some(byte_range)))
}

enum ValueShape {
    Called,
    AssignedFunction,
    Plain,
}

const fn is_call_opener(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::LeftParen
            | TokenKind::LeftBrace
            | TokenKind::StringLiteral(_)
            | TokenKind::InterpolatedStringBegin(_)
    )
}

fn value_shape(tokens: &[Token], index: usize) -> ValueShape {
    match tokens.get(index..) {
        Some([_, next, ..]) if is_call_opener(&next.kind) => ValueShape::Called,
        Some(
            [_, Token { kind: TokenKind::Equal, .. }, Token { kind: TokenKind::Function, .. }, ..],
        ) => ValueShape::AssignedFunction,
        _ => ValueShape::Plain,
    }
}

/// Matches the grammar's all-caps constant pattern: `[A-Z][A-Z0-9_]*`.
fn is_screaming_case(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|rest| rest.is_ascii_uppercase() || rest.is_ascii_digit() || rest == '_')
}

fn encode_tokens(document: &DocumentState, byte_range: Option<Span>) -> SemanticTokens {
    let mut classifier =
        Classifier { document, tokens: document.tokens(), raw_tokens: RawTokens::new() };
    classifier.classify_scope_names();
    classifier.classify_members();
    classifier.visit_block(&document.parsed.block);
    classifier.propagate_to_references();
    classifier.delta_encode(byte_range)
}

/// The classification passes, run in order over one token map.
struct Classifier<'document> {
    document: &'document DocumentState,
    tokens: &'document [Token],
    raw_tokens: RawTokens,
}

impl Classifier<'_> {
    fn classify_scope_names(&mut self) {
        let Self { document, tokens, raw_tokens } = self;
        let stdlib = document.semantic.stdlib();
        let tree = &document.semantic.scope_tree;

        let mut identifier_indexes_by_offset: FxHashMap<u32, usize> = FxHashMap::default();
        for (i, token) in tokens.iter().enumerate() {
            if matches!(token.kind, TokenKind::Identifier(_)) {
                identifier_indexes_by_offset.insert(token.span.start, i);
            }
        }

        let classify_bare = |name: &str, span: Span, base: Classification| -> Classification {
            let mut classification = base;
            if classification.token_type == TYPE_VARIABLE
                && let Some(&index) = identifier_indexes_by_offset.get(&span.start)
            {
                match value_shape(tokens, index) {
                    ValueShape::Called | ValueShape::AssignedFunction => {
                        classification.token_type = TYPE_FUNCTION;
                    }
                    ValueShape::Plain => {
                        if is_screaming_case(name) {
                            classification.modifiers |= MODIFIER_READONLY;
                        }
                    }
                }
            }
            classification
        };

        for symbol in &tree.symbols {
            if symbol.name == "self"
                || !symbol.name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            {
                continue;
            }
            let span = symbol.definition_span;
            let classification = classify_bare(&symbol.name, span, symbol_classify(symbol.kind));
            raw_tokens.insert(span.start, (span, classification));
        }

        for reference in &tree.references {
            // An empty name is a placeholder for what parse recovery dropped,
            // and it can span tokens of its own.
            if reference.name == "self" || reference.name.is_empty() {
                continue;
            }
            let base = reference.resolved.map_or_else(
                || {
                    stdlib
                        .globals
                        .get(reference.name.as_str())
                        .map_or(Classification::plain(TYPE_VARIABLE), stdlib_classify)
                },
                |symbol_id| symbol_classify(tree.symbol(symbol_id).kind),
            );
            let classification = classify_bare(&reference.name, reference.span, base);
            raw_tokens.insert(reference.span.start, (reference.span, classification));
        }
    }

    /// Classifies names after a `.` or `:`. Each one walks its whole chain back
    /// to the root so nested paths classify too: `Enum.Material.Grass`,
    /// `game:GetService`, and `f:read` on a shaped local, not only `math.floor`.
    fn classify_members(&mut self) {
        let Self { document, tokens, raw_tokens } = self;
        for (index, token) in tokens.iter().enumerate() {
            let TokenKind::Identifier(name) = &token.kind else {
                continue;
            };
            let Some((separator, before_separator)) =
                tokens.get(..index).and_then(<[Token]>::split_last)
            else {
                continue;
            };
            if !matches!(separator.kind, TokenKind::Dot | TokenKind::Colon) {
                continue;
            }
            let stdlib_member = dotted_chain_ending_tokens(before_separator).and_then(
                |(mut segments, root_span)| {
                    segments.push(name.as_str());
                    document.semantic.resolve_stdlib_path(&segments, root_span).map(member_classify)
                },
            );
            let classification =
                stdlib_member.unwrap_or_else(|| match value_shape(tokens, index) {
                    ValueShape::Called | ValueShape::AssignedFunction => {
                        Classification::plain(TYPE_METHOD)
                    }
                    ValueShape::Plain => Classification {
                        token_type: TYPE_PROPERTY,
                        modifiers: if is_screaming_case(name) { MODIFIER_READONLY } else { 0 },
                    },
                });
            raw_tokens.insert(token.span.start, (token.span, classification));
        }
    }

    fn propagate_to_references(&mut self) {
        let Self { document, raw_tokens, .. } = self;
        let tree = &document.semantic.scope_tree;
        for symbol in &tree.symbols {
            let Some(&(_, declared)) = raw_tokens.get(&symbol.definition_span.start) else {
                continue;
            };
            for reference_id in &symbol.reference_ids {
                let reference = tree.reference(*reference_id);
                if let Some((_, classification)) = raw_tokens.get_mut(&reference.span.start) {
                    if !symbol.is_modified && declared.token_type == TYPE_FUNCTION {
                        classification.token_type = TYPE_FUNCTION;
                    }
                    classification.modifiers |= declared.modifiers & MODIFIER_READONLY;
                }
            }
        }
    }

    fn delta_encode(self, byte_range: Option<Span>) -> SemanticTokens {
        let Self { document, raw_tokens, .. } = self;
        let mut data: Vec<SemanticToken> = Vec::with_capacity(raw_tokens.len());
        let mut previous = Position { line: 0, character: 0 };
        for (span, classification) in raw_tokens.into_values() {
            if span.is_empty()
                || byte_range.is_some_and(|Span { start, end }| {
                    start >= end || span.end <= start || span.start >= end
                })
            {
                continue;
            }
            let Position { line, character } = document.position(span.start);
            let delta_line = line.saturating_sub(previous.line);
            let delta_start = if delta_line == 0 {
                character.saturating_sub(previous.character)
            } else {
                character
            };
            data.push(SemanticToken {
                delta_line,
                delta_start,
                length: utf16_length(document.text(), span),
                token_type: classification.token_type,
                token_modifiers_bitset: classification.modifiers,
            });
            previous = Position { line, character };
        }

        SemanticTokens { result_id: None, data }
    }

    fn insert(&mut self, span: Span, token_type: u32) {
        self.raw_tokens.insert(span.start, (span, Classification::plain(token_type)));
    }

    fn retype(&mut self, span: Span, token_type: u32) {
        if let Some((_, classification)) = self.raw_tokens.get_mut(&span.start) {
            classification.token_type = token_type;
        }
    }

    /// The name a call calls, through parentheses, casts, and Luau type
    /// instantiations.
    fn callee(&mut self, callee: ChainHead<'_>) {
        let mut current = callee;
        loop {
            current = match current.split_last() {
                Some((_, Suffix::Field(name))) => {
                    self.retype(name.span, TYPE_METHOD);
                    return;
                }
                Some((before, Suffix::TypeInstantiation { .. })) => before, // Luau
                Some((_, Suffix::Index { .. } | Suffix::Call(_))) => return,
                None => match current.prefix {
                    Expression::Name(name) => {
                        self.retype(name.span, TYPE_FUNCTION);
                        return;
                    }
                    Expression::Parenthesized(parenthesized) => {
                        ChainHead::from(&parenthesized.expr)
                    }
                    Expression::TypeCast(cast) => ChainHead::from(&cast.expr), // Luau
                    Expression::Nil(_)
                    | Expression::False(_)
                    | Expression::True(_)
                    | Expression::Number(_)
                    | Expression::Integer(_)
                    | Expression::StringLiteral(_)
                    | Expression::VarArg(_)
                    | Expression::FunctionDef(_)
                    | Expression::Suffixed(_)
                    | Expression::TableConstructor(_)
                    | Expression::BinaryOp(_)
                    | Expression::UnaryOp(_)
                    | Expression::IfExpression(_)
                    | Expression::InterpolatedString(_)
                    | Expression::Error(_) => return,
                },
            };
        }
    }
}

impl<'ast> Visitor<'ast> for Classifier<'_> {
    fn visit_stmt(&mut self, stmt: &'ast Statement) {
        match stmt {
            // Luau
            Statement::TypeDeclaration(declaration) => {
                let token_type = match &declaration.type_value {
                    TypeDeclarationValue::Alias(_) => TYPE_TYPE,
                    TypeDeclarationValue::TypeFunction(_) => TYPE_FUNCTION,
                };
                self.insert(declaration.name.span, token_type);
            }
            Statement::FunctionDecl(declaration) => {
                if let Some(name) =
                    declaration.name.method.as_ref().or_else(|| declaration.name.names.last())
                {
                    self.insert(
                        name.span,
                        if declaration.name.method.is_some() || declaration.name.names.len() > 1 {
                            TYPE_METHOD
                        } else {
                            TYPE_FUNCTION
                        },
                    );
                }
            }
            Statement::LocalAssignment(assignment) => {
                for (index, name) in assignment.names.iter().enumerate() {
                    if assignment
                        .exprs
                        .as_ref()
                        .and_then(|exprs| exprs.get(index))
                        .is_some_and(|expr| matches!(expr, Expression::FunctionDef(_)))
                    {
                        self.insert(name.name.span, TYPE_FUNCTION);
                    }
                    // Lua 5.4+: a to-be-closed local can't be assigned either.
                    let is_read_only = assignment.is_const
                        || name.attribute.as_ref().is_some_and(|attribute| {
                            matches!(attribute.name.identifier(), Some("const" | "close"))
                        });
                    if is_read_only
                        && let Some((_, classification)) =
                            self.raw_tokens.get_mut(&name.name.span.start)
                    {
                        classification.modifiers |= MODIFIER_READONLY;
                    }
                }
            }
            // Lua 5.5
            Statement::GlobalFunction(global_function) => {
                if global_function.name.identifier().is_some() {
                    self.insert(global_function.name.span, TYPE_FUNCTION);
                }
            }
            Statement::Assignment(_)
            | Statement::FunctionCall(_)
            | Statement::DoBlock(_)
            | Statement::WhileLoop(_)
            | Statement::RepeatLoop(_)
            | Statement::IfStatement(_)
            | Statement::NumericFor(_)
            | Statement::GenericFor(_)
            | Statement::LocalFunction(_)
            | Statement::CompoundAssignment(_)
            | Statement::GlobalDeclaration(_)
            | Statement::GlobalStar(_)
            | Statement::Goto(_)
            | Statement::Label(_)
            | Statement::EmptyStatement(_)
            | Statement::Break(_)
            | Statement::Error(_) => {}
        }
        self.walk_stmt(stmt);
    }

    fn visit_call(&mut self, call: Call<'ast>) {
        match call.method {
            Some(method) => self.retype(method.span, TYPE_METHOD),
            None => self.callee(call.callee),
        }
        self.walk_call(call);
    }

    fn visit_table_constructor(&mut self, table: &'ast TableConstructor) {
        for field in table.fields.iter() {
            match field {
                Field::Named { name, value, .. } => {
                    self.insert(
                        name.span,
                        if matches!(value, Expression::FunctionDef(_)) {
                            TYPE_METHOD
                        } else {
                            TYPE_PROPERTY
                        },
                    );
                    self.visit_expr(value);
                }
                Field::Bracketed { key, value, .. } => {
                    self.visit_expr(key);
                    self.visit_expr(value);
                }
                Field::Positional { value, .. } => self.visit_expr(value),
            }
        }
    }

    fn visit_generic_type_list(&mut self, generics: &'ast GenericTypeList) {
        for param in generics.params.iter() {
            self.insert(param.name.span, TYPE_TYPE);
            if let Some(default) = &param.default {
                self.visit_type(default);
            }
        }
    }

    fn visit_type(&mut self, type_value: &'ast Type) {
        match type_value {
            Type::Name { name, span } => {
                if matches!(
                    name.as_str(),
                    "any"
                        | "unknown"
                        | "never"
                        | "boolean"
                        | "number"
                        | "integer"
                        | "string"
                        | "thread"
                        | "userdata"
                        | "buffer"
                        | "vector"
                ) {
                    self.raw_tokens.remove(&span.start);
                } else {
                    self.insert(*span, TYPE_TYPE);
                }
            }
            Type::Typeof(query) => {
                self.raw_tokens.remove(&query.span.start);
            }
            Type::Named(named) => {
                if let Some(prefix) = &named.prefix {
                    self.insert(prefix.span, TYPE_VARIABLE);
                }
                self.insert(named.name.span, TYPE_TYPE);
            }
            Type::GenericPack(pack) => self.insert(pack.name.span, TYPE_TYPE),
            Type::Table(table) => {
                for field in table.fields.iter() {
                    match field {
                        TypeField::Named { name, .. } => self.insert(name.span, TYPE_PROPERTY),
                        TypeField::Indexer { .. } | TypeField::Array { .. } => {}
                    }
                }
            }
            Type::Function(function_type) => {
                for param in function_type.params.iter() {
                    if let Some(name) = &param.name {
                        self.insert(name.span, TYPE_PARAMETER);
                    }
                }
            }
            Type::Optional(_)
            | Type::Union(_)
            | Type::Intersection(_)
            | Type::Parenthesized(_)
            | Type::Pack(_)
            | Type::Variadic(_)
            | Type::Singleton(_)
            | Type::Error(_) => {}
        }
        self.walk_type(type_value);
    }
}

const fn symbol_classify(kind: SymbolKind) -> Classification {
    match kind {
        SymbolKind::Parameter | SymbolKind::ImplicitSelf => Classification::plain(TYPE_PARAMETER),
        SymbolKind::FunctionName => Classification::plain(TYPE_FUNCTION),
        SymbolKind::Local | SymbolKind::IteratorVariable | SymbolKind::NumericForVariable => {
            Classification::plain(TYPE_VARIABLE)
        }
    }
}

fn stdlib_classify(entry: &StdlibEntry) -> Classification {
    let token_type = match entry {
        StdlibEntry::Function(_) => TYPE_FUNCTION,
        StdlibEntry::Namespace(_) => TYPE_NAMESPACE,
        StdlibEntry::Constant(_) | StdlibEntry::Property(_) => TYPE_PROPERTY,
    };
    let mut modifiers = MODIFIER_DEFAULT_LIBRARY;
    if entry.deprecation().is_some() {
        modifiers |= MODIFIER_DEPRECATED;
    }
    if entry.is_read_only() {
        modifiers |= MODIFIER_READONLY;
    }
    Classification { token_type, modifiers }
}

/// Stdlib namespace members: same modifiers as bare globals, but member
/// functions read as methods so call sites match the accessor grammar.
fn member_classify(entry: &StdlibEntry) -> Classification {
    let mut classification = stdlib_classify(entry);
    if classification.token_type == TYPE_FUNCTION {
        classification.token_type = TYPE_METHOD;
    }
    classification
}

fn utf16_length(source: &str, span: Span) -> u32 {
    let end = offset_to_index(span.end).min(source.len());
    source
        .get(offset_to_index(span.start)..end)
        .map_or(0, |text| index_to_offset(text.encode_utf16().count()))
}

#[cfg(test)]
mod tests {
    use luck_core::LuaTarget;
    use rustc_hash::FxHashSet;

    use super::*;

    fn parse_document(source: &str, target: LuaTarget) -> DocumentState {
        let document = DocumentState::new(source.to_owned(), 1, target, FxHashSet::default());
        assert!(document.parsed.errors.is_empty(), "{:?}", document.parsed.errors);
        document
    }

    fn full_tokens(document: &DocumentState) -> Vec<SemanticToken> {
        let SemanticTokensResult::Tokens(tokens) = semantic_tokens(document) else {
            panic!("expected full tokens");
        };
        tokens.data
    }

    fn range_tokens(document: &DocumentState, range: Range) -> Vec<SemanticToken> {
        let SemanticTokensRangeResult::Tokens(tokens) = in_range(document, range) else {
            panic!("expected range tokens");
        };
        tokens.data
    }

    #[derive(Debug)]
    struct Highlight {
        text: String,
        kind: String,
        modifiers: Vec<String>,
        position: Position,
    }

    fn highlights(document: &DocumentState) -> Vec<Highlight> {
        let data = full_tokens(document);
        let legend = legend();
        let mut position = Position::default();
        let mut previous_end = Position::default();
        data.iter()
            .map(|token| {
                position.line = position.line.saturating_add(token.delta_line);
                position.character = if token.delta_line == 0 {
                    position.character.saturating_add(token.delta_start)
                } else {
                    token.delta_start
                };
                assert!(position >= previous_end, "overlapping tokens: {data:?}");
                assert!(token.length > 0, "zero-length token: {data:?}");
                previous_end =
                    Position::new(position.line, position.character.saturating_add(token.length));
                let span = Span::new(document.offset(position), document.offset(previous_end));
                Highlight {
                    text: span.text(document.text()).to_owned(),
                    kind: legend
                        .token_types
                        .get(offset_to_index(token.token_type))
                        .map_or_else(String::new, |kind| kind.as_str().to_owned()),
                    modifiers: legend
                        .token_modifiers
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| token.token_modifiers_bitset & (1 << i) != 0)
                        .map(|(_, modifier)| modifier.as_str().to_owned())
                        .collect(),
                    position,
                }
            })
            .collect()
    }

    #[test]
    fn all_targets_color_globals_locals_properties_and_calls() {
        for target in [
            LuaTarget::Lua51,
            LuaTarget::Lua52,
            LuaTarget::Lua53,
            LuaTarget::Lua54,
            LuaTarget::Lua55,
            LuaTarget::Luau,
            LuaTarget::LuauRoblox,
        ] {
            let document = parse_document(
                "Players = Services.Players\nlocal object = {field = Players}\nif not object:IsA('Tool') and not object:IsA('HopperBin') then\n  if 0 < #getPlayer(arg1[1], arg2) then print(object.field) end\nend",
                target,
            );
            let tokens = highlights(&document);
            for (name, kind) in [
                ("Players", "variable"),
                ("Services", "variable"),
                ("object", "variable"),
                ("field", "property"),
                ("IsA", "method"),
                ("getPlayer", "function"),
                ("arg1", "variable"),
                ("arg2", "variable"),
            ] {
                let token = tokens.iter().find(|token| token.text == name).expect(name);
                assert_eq!(token.kind, kind, "{target:?}: {tokens:?}");
            }
            assert_eq!(
                tokens
                    .iter()
                    .filter(|token| token.text == "Players" && token.kind == "property")
                    .count(),
                1,
                "{tokens:?}"
            );
            assert!(
                tokens
                    .iter()
                    .all(|token| !["not", "and", "if", "then", "<"].contains(&token.text.as_str())),
                "keywords must retain grammar colors: {tokens:?}"
            );
        }
    }

    #[test]
    fn type_names_and_table_keys_are_not_value_properties_or_globals() {
        let document = parse_document(
            "type Alias<T> = {field: module.Member<T>, callback: (arg: T) -> T}\nlocal value: Alias<number> = {}\nlocal item = {field = value}\nlocal chosen = value :: module.Member<number>\n",
            LuaTarget::Luau,
        );
        let tokens = highlights(&document);
        for name in ["Alias", "T", "Member"] {
            for token in tokens.iter().filter(|token| token.text == name) {
                assert_eq!(token.kind, "type", "{tokens:?}");
            }
        }
        for token in tokens.iter().filter(|token| token.text == "field" || token.text == "callback")
        {
            assert_eq!(token.kind, "property", "{tokens:?}");
        }
        assert!(
            tokens.iter().any(|token| token.text == "arg" && token.kind == "parameter"),
            "{tokens:?}"
        );
        assert!(
            tokens.iter().all(|token| token.text != "type" && token.text != "number"),
            "{tokens:?}"
        );
    }

    #[test]
    fn primitive_annotations_and_type_queries_keep_their_grammar_scopes() {
        let document = parse_document(
            "local value: number = 1\nlocal text: string = ''\nlocal shape: typeof(value) = value\nfunction handler(arg: boolean): number return value end",
            LuaTarget::Luau,
        );
        let tokens = highlights(&document);
        assert!(
            tokens
                .iter()
                .all(|token| !["number", "string", "boolean", "typeof"]
                    .contains(&token.text.as_str())),
            "{tokens:?}"
        );
        assert!(
            tokens.iter().any(|token| token.text == "value" && token.position.line == 2),
            "{tokens:?}"
        );
    }

    #[test]
    fn functions_keep_their_color_in_annotations_references_and_instantiations() {
        let document = parse_document(
            "local callback: (number) -> number = function(value) return value end\nlocal saved = callback\nfunction object:method<T>(value: T): T return value end\nobject:method<<number>>(callback<<number>>(1))\n",
            LuaTarget::Luau,
        );
        let tokens = highlights(&document);
        for token in tokens.iter().filter(|token| token.text == "callback") {
            assert_eq!(token.kind, "function", "{tokens:?}");
        }
        for token in tokens.iter().filter(|token| token.text == "method") {
            assert_eq!(token.kind, "method", "{tokens:?}");
        }
    }

    #[test]
    fn shadowed_stdlib_aliases_do_not_inherit_default_library() {
        let document = parse_document(
            "local print = 1\nlocal alias = print\nlocal math = {floor = function(value) return value end}\nmath.floor(alias)\n",
            LuaTarget::Lua54,
        );
        let tokens = highlights(&document);
        assert!(
            tokens
                .iter()
                .all(|token| !token.modifiers.iter().any(|modifier| modifier == "defaultLibrary")),
            "{tokens:?}"
        );
    }

    #[test]
    fn real_readonly_bindings_propagate_to_references() {
        for (source, target) in [
            ("local limit <const> = 1\nprint(limit)", LuaTarget::Lua54),
            ("const limit = 1\nprint(limit)", LuaTarget::Luau),
        ] {
            let tokens = highlights(&parse_document(source, target));
            for token in tokens.iter().filter(|token| token.text == "limit") {
                assert!(
                    token.modifiers.iter().any(|modifier| modifier == "readonly"),
                    "{tokens:?}"
                );
            }
        }
    }

    #[test]
    fn utf16_positions_in_interpolations_and_half_open_ranges_across_crlf() {
        let document = parse_document(
            "local text = `\u{1F600} {object.field}`\r\nPlayers = Services.Players\r\n",
            LuaTarget::Luau,
        );
        let tokens = highlights(&document);
        let object = tokens.iter().find(|token| token.text == "object").expect("object");
        assert_eq!(object.position, Position::new(0, 18), "{tokens:?}");
        for range in [
            Range::new(Position::new(1, 0), Position::new(1, 0)),
            Range::new(Position::new(1, 7), Position::new(1, 10)),
        ] {
            let whitespace = range_tokens(&document, range);
            assert!(whitespace.is_empty(), "half-open whitespace range: {whitespace:?}");
        }
        let ranged =
            range_tokens(&document, Range::new(Position::new(1, 10), Position::new(1, 18)));
        assert_eq!(ranged.len(), 1, "{ranged:?}");
        assert_eq!(ranged[0].delta_line, 1);
        assert_eq!(ranged[0].delta_start, 10);
        assert_eq!(ranged[0].length, 8);
    }

    #[test]
    fn semantic_tokens_classify_nested_enum_item() {
        let document = parse_document("local m = Enum.Material.Grass", LuaTarget::LuauRoblox);
        let tokens = full_tokens(&document);
        // m, Enum, Material, Grass. The last token is `Grass`; it must be
        // classified as a stdlib property (defaultLibrary | readonly = 6),
        // not a plain property.
        let grass = tokens.last().expect("tokens present");
        assert_eq!(grass.length, index_to_offset("Grass".len()));
        assert_eq!(grass.token_type, 2, "property: {tokens:?}");
        assert_eq!(
            grass.token_modifiers_bitset & MODIFIER_DEFAULT_LIBRARY,
            MODIFIER_DEFAULT_LIBRARY,
            "defaultLibrary modifier expected: {tokens:?}"
        );
    }

    #[test]
    fn deprecated_namespaces_carry_the_deprecated_modifier() {
        let document = parse_document("local g = Enum.Genre\n", LuaTarget::LuauRoblox);
        let tokens = highlights(&document);
        let genre = tokens.iter().find(|token| token.text == "Genre").expect("Genre");
        assert_eq!(genre.kind, "namespace", "{tokens:?}");
        for modifier in ["deprecated", "readonly", "defaultLibrary"] {
            assert!(genre.modifiers.iter().any(|name| name == modifier), "{modifier}: {tokens:?}");
        }
        let live = parse_document("local m = Enum.Material\n", LuaTarget::LuauRoblox);
        let live_tokens = highlights(&live);
        let material = live_tokens.iter().find(|token| token.text == "Material").expect("Material");
        assert!(!material.modifiers.iter().any(|name| name == "deprecated"), "{live_tokens:?}");
    }

    #[test]
    fn semantic_tokens_classify_shaped_method_calls() {
        let document =
            parse_document("local f = io.open('x')\nlocal n = f:read('n')", LuaTarget::Lua54);
        let tokens = full_tokens(&document);
        // `read` must be METHOD (5) with defaultLibrary (1 << 2).
        let read = tokens
            .iter()
            .find(|token| token.length == 4 && token.token_type == 5)
            .unwrap_or_else(|| panic!("method token for read: {tokens:?}"));
        assert_eq!(
            read.token_modifiers_bitset & MODIFIER_DEFAULT_LIBRARY,
            MODIFIER_DEFAULT_LIBRARY,
            "defaultLibrary expected on shaped method: {tokens:?}"
        );
    }

    #[test]
    fn semantic_tokens_classify_get_service_method() {
        let document =
            parse_document("local p = game:GetService('RunService')", LuaTarget::LuauRoblox);
        let tokens = full_tokens(&document);
        let get_service = tokens
            .iter()
            .find(|token| token.length == index_to_offset("GetService".len()))
            .expect("GetService token");
        assert_eq!(get_service.token_type, 5, "method: {tokens:?}");
        assert_eq!(
            get_service.token_modifiers_bitset & MODIFIER_DEFAULT_LIBRARY,
            MODIFIER_DEFAULT_LIBRARY,
            "defaultLibrary expected: {tokens:?}"
        );
    }

    #[test]
    fn a_to_be_closed_local_is_readonly() {
        // Manual 3.3.8: a to-be-closed variable behaves like a constant local.
        let tokens = highlights(&parse_document("local h <close> = nil\n", LuaTarget::Lua54));
        let declaration = tokens.iter().find(|token| token.text == "h").expect("h");
        assert!(
            declaration.modifiers.iter().any(|modifier| modifier == "readonly"),
            "`<close>` can't be assigned, same as `<const>`: {tokens:?}"
        );
    }

    #[test]
    fn a_global_function_name_is_a_function_token() {
        let tokens =
            highlights(&parse_document("global function setup()\nend\n", LuaTarget::Lua55));
        let name = tokens.iter().find(|token| token.text == "setup").expect("setup");
        assert_eq!(name.kind, "function", "{tokens:?}");
    }

    #[test]
    fn a_recovered_assignment_target_emits_no_overlapping_token() {
        // `a.b(c)` can't be assigned, so recovery names it with an empty
        // identifier spanning the whole call. `highlights` asserts that no
        // two tokens overlap and that each has a length.
        let document = DocumentState::new(
            "a.b(c), d = 1\n".to_owned(),
            1,
            LuaTarget::Lua54,
            FxHashSet::default(),
        );
        assert!(!document.parsed.errors.is_empty(), "the target must be invalid");
        let tokens = highlights(&document);
        assert!(
            tokens.iter().all(|token| !token.text.contains('(')),
            "a token must cover one identifier: {tokens:?}"
        );
    }
}
