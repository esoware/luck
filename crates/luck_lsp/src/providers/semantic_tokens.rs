//! Identifier highlighting layered over the `TextMate` grammar. Scope
//! resolution handles values; the cached AST distinguishes types and table
//! keys from superficially identical value expressions.

use alloc::collections::BTreeMap;
use core::iter;

use luck_ast::{
    ChainHead, Expression, Field, GenericTypeList, Statement, Suffix, SuffixedExpression,
    TableConstructor, Type, TypeDeclarationValue, TypeField, Visit,
};
use luck_semantic::{SemanticAnalysis, StdlibEntry, SymbolKind};
use luck_token::{Span, Token, TokenKind, index_to_offset, offset_to_index};
use tower_lsp_server::ls_types::{
    Position, Range, SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens,
    SemanticTokensLegend, SemanticTokensRangeResult, SemanticTokensResult,
};

use crate::document::DocumentState;
use crate::providers::cursor::identifier;

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
pub(crate) struct Classification {
    token_type: u32,
    modifiers: u32,
}

impl Classification {
    const fn plain(token_type: u32) -> Self {
        Self { token_type, modifiers: 0 }
    }
}

/// A document's classified names, by where they start, none empty.
pub(crate) type Highlights = Vec<(Span, Classification)>;

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

const fn is_call_opener(kind: TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::LeftParen
            | TokenKind::LeftBrace
            | TokenKind::StringLiteral
            | TokenKind::InterpolatedStringBegin
            | TokenKind::InterpolatedStringSimple
    )
}

fn value_shape(tokens: &[Token], index: usize) -> ValueShape {
    match tokens.get(index..) {
        Some([_, next, ..]) if is_call_opener(next.kind) => ValueShape::Called,
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

/// Runs the classification passes over `document`, in order, each later one
/// overwriting what an earlier one gave the same start.
pub(crate) fn classify(document: &DocumentState) -> Highlights {
    let tokens = document.tokens();
    let mut classifier = Classifier {
        source: document.text(),
        tokens,
        semantic: document.semantic(),
        slots: vec![None; tokens.len()],
        strays: BTreeMap::new(),
        symbol_keys: Vec::new(),
        reference_keys: Vec::new(),
        last_found: 0,
    };
    classifier.classify_scope_names();
    classifier.classify_members();
    classifier.visit_block(&document.parse().block);
    classifier.propagate_to_references();
    classifier.finish()
}

fn encode_tokens(document: &DocumentState, byte_range: Option<Span>) -> SemanticTokens {
    let highlights = document.highlights();
    let mut data: Vec<SemanticToken> = Vec::with_capacity(highlights.len());
    let mut positions = document.positions();
    let mut previous = Position { line: 0, character: 0 };
    for &(span, classification) in highlights {
        if byte_range.is_some_and(|Span { start, end }| {
            start >= end || span.end <= start || span.start >= end
        }) {
            continue;
        }
        let Position { line, character } = positions.position(span.start);
        let delta_line = line.saturating_sub(previous.line);
        let delta_start =
            if delta_line == 0 { character.saturating_sub(previous.character) } else { character };
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

/// How many tokens past the last one found a lookup tries before it
/// searches.
const NEARBY_TOKENS: usize = 8;

/// Where a classification is kept: the slot of the token that starts there,
/// or, where none does, which only a name parse recovery made up can reach,
/// the strays.
#[derive(Debug, Clone, Copy)]
enum Key {
    Token(usize),
    Stray(u32),
}

/// Where a name stands in the `Name(.Name)*` chain it ends.
#[derive(Clone, Copy)]
enum ChainLink {
    Root,
    /// What the chain resolves to in the stdlib, if anything.
    Member(Option<&'static StdlibEntry>),
}

/// The classification passes, run in order over one table of
/// classifications.
struct Classifier<'document> {
    source: &'document str,
    tokens: &'document [Token],
    semantic: &'document SemanticAnalysis<'document>,
    /// Each token's classification, by token index, with the span it was
    /// given.
    slots: Vec<Option<(Span, Classification)>>,
    strays: BTreeMap<u32, (Span, Classification)>,
    /// Where each symbol's declaration and each reference is kept, found
    /// once in the first pass for the last.
    symbol_keys: Vec<Key>,
    reference_keys: Vec<Key>,
    /// The token the last lookup found. The passes mostly walk the source
    /// forward, so the tokens after it are tried before a search.
    last_found: usize,
}

impl Classifier<'_> {
    fn key(&mut self, start: u32) -> Key {
        let ahead = self.tokens.get(self.last_found..).unwrap_or_default();
        let nearby = ahead.iter().take(NEARBY_TOKENS).position(|token| token.span.start == start);
        let found = nearby.map(|i| self.last_found.saturating_add(i)).map_or_else(
            || self.tokens.binary_search_by_key(&start, |token| token.span.start).ok(),
            Some,
        );
        let Some(index) = found else {
            return Key::Stray(start);
        };
        self.last_found = index;
        Key::Token(index)
    }

    fn get(&self, key: Key) -> Option<&(Span, Classification)> {
        match key {
            Key::Token(index) => self.slots.get(index)?.as_ref(),
            Key::Stray(start) => self.strays.get(&start),
        }
    }

    fn get_mut(&mut self, key: Key) -> Option<&mut (Span, Classification)> {
        match key {
            Key::Token(index) => self.slots.get_mut(index)?.as_mut(),
            Key::Stray(start) => self.strays.get_mut(&start),
        }
    }

    fn set(&mut self, key: Key, span: Span, classification: Classification) {
        match key {
            Key::Token(index) => {
                if let Some(slot) = self.slots.get_mut(index) {
                    *slot = Some((span, classification));
                }
            }
            Key::Stray(start) => {
                self.strays.insert(start, (span, classification));
            }
        }
    }

    fn remove(&mut self, span: Span) {
        match self.key(span.start) {
            Key::Token(index) => {
                if let Some(slot) = self.slots.get_mut(index) {
                    *slot = None;
                }
            }
            Key::Stray(start) => {
                self.strays.remove(&start);
            }
        }
    }

    fn insert(&mut self, span: Span, token_type: u32) {
        let key = self.key(span.start);
        self.set(key, span, Classification::plain(token_type));
    }

    fn retype(&mut self, span: Span, token_type: u32) {
        let key = self.key(span.start);
        if let Some((_, classification)) = self.get_mut(key) {
            classification.token_type = token_type;
        }
    }

    /// A plain variable becomes a function where it's called or assigned
    /// one, and reads as read-only where it's spelled like a constant.
    fn classify_bare(&self, name: &str, key: Key, base: Classification) -> Classification {
        let mut classification = base;
        if classification.token_type == TYPE_VARIABLE
            && let Key::Token(index) = key
            && self.tokens.get(index).is_some_and(|token| token.kind == TokenKind::Identifier)
        {
            match value_shape(self.tokens, index) {
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
    }

    fn classify_scope_names(&mut self) {
        let semantic = self.semantic;
        let tree = semantic.scope_tree();
        let stdlib = semantic.stdlib();

        self.symbol_keys.reserve_exact(tree.symbols().len());
        for symbol in tree.symbols() {
            let span = symbol.definition_span;
            let key = self.key(span.start);
            self.symbol_keys.push(key);
            let name = tree.name(symbol.name);
            if name == "self" || !name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
                continue;
            }
            let classification = self.classify_bare(name, key, symbol_classify(symbol.kind));
            self.set(key, span, classification);
        }

        let self_name = tree.name_id("self");
        // Each name's classification as a global, so a name read many times
        // is looked up once.
        let globals: Vec<Classification> = tree
            .names()
            .map(|(_, name)| {
                stdlib
                    .globals
                    .get(name)
                    .map_or(Classification::plain(TYPE_VARIABLE), stdlib_classify)
            })
            .collect();
        self.reference_keys.reserve_exact(tree.references().len());
        for reference in tree.references() {
            let key = self.key(reference.span.start);
            self.reference_keys.push(key);
            if Some(reference.name) == self_name {
                continue;
            }
            let name = tree.name(reference.name);
            let base = reference.resolved.map_or_else(
                || {
                    let global = globals.get(reference.name.index()).copied();
                    global.unwrap_or(Classification::plain(TYPE_VARIABLE))
                },
                |symbol_id| symbol_classify(tree.symbol(symbol_id).kind),
            );
            let classification = self.classify_bare(name, key, base);
            self.set(key, reference.span, classification);
        }
    }

    /// Classifies names after a `.` or `:`, resolving each one's whole chain
    /// back to its root, so nested paths classify too: `Enum.Material.Grass`,
    /// `game:GetService`, and `f:read` on a shaped local, not only
    /// `math.floor`. A path resolves one member at a time, so each name
    /// resolves from the one before it rather than walking its chain again.
    fn classify_members(&mut self) {
        let Self { source, tokens, semantic, .. } = *self;
        let library = semantic.stdlib();
        let tree = semantic.scope_tree();
        // By where they start, so a root's is found by walking on in step
        // with the tokens.
        let whole_source = Span::new(0, index_to_offset(source.len()));
        let mut references =
            tree.references_within(whole_source).map(|id| tree.reference(id)).peekable();
        // A name's path continues the one two tokens back, past the separator.
        let (mut link_two_back, mut link_one_back): (Option<ChainLink>, Option<ChainLink>) =
            (None, None);
        for (index, token) in tokens.iter().enumerate() {
            let link_before = link_two_back;
            link_two_back = link_one_back;
            if token.kind != TokenKind::Identifier {
                link_one_back = None;
                continue;
            }
            let name = token.text(source);
            let separator = index.checked_sub(1).and_then(|before| tokens.get(before));
            let is_after_dot = separator.is_some_and(|before| before.kind == TokenKind::Dot);
            let is_member =
                is_after_dot || separator.is_some_and(|before| before.kind == TokenKind::Colon);
            if !is_member {
                link_one_back = Some(ChainLink::Root);
                continue;
            }
            let chain_before =
                link_before.and_then(|link| Some((tokens.get(index.checked_sub(2)?)?, link)));
            let entry = chain_before.and_then(|(before, link)| match link {
                ChainLink::Root => {
                    let root = before.text(source);
                    let start = before.span.start;
                    let reference = iter::from_fn(|| {
                        references.next_if(|reference| reference.span.start <= start)
                    })
                    .find(|reference| {
                        reference.span == before.span && tree.name(reference.name) == root
                    })?;
                    semantic.resolve_stdlib_path(&[root, name], reference)
                }
                ChainLink::Member(entry) => library.child(entry?, name),
            });
            link_one_back = if is_after_dot {
                chain_before.map(|_| ChainLink::Member(entry))
            } else {
                Some(ChainLink::Root)
            };
            let classification = entry.map_or_else(
                || match value_shape(tokens, index) {
                    ValueShape::Called | ValueShape::AssignedFunction => {
                        Classification::plain(TYPE_METHOD)
                    }
                    ValueShape::Plain => Classification {
                        token_type: TYPE_PROPERTY,
                        modifiers: if is_screaming_case(name) { MODIFIER_READONLY } else { 0 },
                    },
                },
                member_classify,
            );
            self.set(Key::Token(index), token.span, classification);
        }
    }

    fn propagate_to_references(&mut self) {
        let tree = self.semantic.scope_tree();
        for symbol in tree.symbols() {
            let declared_key = self.symbol_keys.get(symbol.id.index()).copied();
            let Some(&(_, declared)) = declared_key.and_then(|key| self.get(key)) else {
                continue;
            };
            for reference_id in tree.resolved_references(symbol.id) {
                let Some(&key) = self.reference_keys.get(reference_id.index()) else {
                    continue;
                };
                if let Some((_, classification)) = self.get_mut(key) {
                    if !symbol.is_modified && declared.token_type == TYPE_FUNCTION {
                        classification.token_type = TYPE_FUNCTION;
                    }
                    classification.modifiers |= declared.modifiers & MODIFIER_READONLY;
                }
            }
        }
    }

    /// Every classification by where it starts, those with no text dropped.
    fn finish(self) -> Highlights {
        let mut highlights = Vec::with_capacity(self.slots.len().div_ceil(2));
        let mut strays = self.strays.into_values().filter(|(span, _)| !span.is_empty()).peekable();
        for slot in self.slots.into_iter().flatten().filter(|(span, _)| !span.is_empty()) {
            while let Some(stray) = strays.next_if(|(span, _)| span.start < slot.0.start) {
                highlights.push(stray);
            }
            highlights.push(slot);
        }
        highlights.extend(strays);
        highlights
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
                Some((_, Suffix::Index { .. } | Suffix::Call(_) | Suffix::MethodCall(_))) => {
                    return;
                }
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

impl<'ast> Visit<'ast> for Classifier<'_> {
    fn visit_stmt(&mut self, stmt: &'ast Statement<'ast>) {
        match stmt {
            // Luau
            Statement::TypeDeclaration(declaration) => {
                let token_type = match &declaration.value {
                    TypeDeclarationValue::Alias(_) => TYPE_TYPE,
                    TypeDeclarationValue::TypeFunction(_) => TYPE_FUNCTION,
                };
                self.insert(declaration.name.span, token_type);
            }
            Statement::FunctionDecl(declaration) => {
                let name = &declaration.name;
                if let Some(last) = name.method.as_ref().or_else(|| name.names.last()) {
                    let is_method = name.method.is_some() || name.names.len() > 1;
                    self.insert(last.span, if is_method { TYPE_METHOD } else { TYPE_FUNCTION });
                }
            }
            Statement::LocalAssignment(assignment) => {
                for (index, attributed) in assignment.names.iter().enumerate() {
                    if matches!(assignment.exprs.get(index), Some(Expression::FunctionDef(_))) {
                        self.insert(attributed.name.span, TYPE_FUNCTION);
                    }
                    // Lua 5.4+: a to-be-closed local can't be assigned either.
                    let is_read_only = assignment.kind.is_const()
                        || attributed.attribute.as_ref().is_some_and(|attribute| {
                            matches!(identifier(&attribute.name), Some("const" | "close"))
                        });
                    let key = self.key(attributed.name.span.start);
                    if is_read_only && let Some((_, classification)) = self.get_mut(key) {
                        classification.modifiers |= MODIFIER_READONLY;
                    }
                }
            }
            // Lua 5.5
            Statement::GlobalFunction(global_function) => {
                if identifier(&global_function.name).is_some() {
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

    fn visit_suffixed_expr(&mut self, suffixed_expr: &'ast SuffixedExpression<'ast>) {
        for call in suffixed_expr.calls() {
            match call.method {
                Some(method) => self.retype(method.span, TYPE_METHOD),
                None => self.callee(call.callee),
            }
        }
        self.walk_suffixed_expr(suffixed_expr);
    }

    fn visit_table_constructor(&mut self, table_constructor: &'ast TableConstructor<'ast>) {
        for field in &table_constructor.fields {
            match field {
                Field::Named { name, value } => {
                    let is_method = matches!(value, Expression::FunctionDef(_));
                    self.insert(name.span, if is_method { TYPE_METHOD } else { TYPE_PROPERTY });
                    self.visit_expr(value);
                }
                Field::Bracketed { key, value, .. } => {
                    self.visit_expr(key);
                    self.visit_expr(value);
                }
                Field::Positional(value) => self.visit_expr(value),
            }
        }
    }

    fn visit_generic_type_list(&mut self, generic_type_list: &'ast GenericTypeList<'ast>) {
        for param in &generic_type_list.params {
            self.insert(param.name.span, TYPE_TYPE);
            if let Some(default) = &param.default {
                self.visit_type(default);
            }
        }
    }

    fn visit_type(&mut self, ty: &'ast Type<'ast>) {
        match ty {
            Type::Name(name) => {
                if matches!(
                    name.text.as_str(),
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
                    self.remove(name.span);
                } else {
                    self.insert(name.span, TYPE_TYPE);
                }
            }
            Type::Typeof(query) => self.remove(query.span),
            Type::Named(named) => {
                if let Some(prefix) = &named.prefix {
                    self.insert(prefix.span, TYPE_VARIABLE);
                }
                self.insert(named.name.span, TYPE_TYPE);
            }
            Type::GenericPack(pack) => self.insert(pack.name.span, TYPE_TYPE),
            Type::Table(table) => {
                for field in &table.fields {
                    match field {
                        TypeField::Named { name, .. } => self.insert(name.span, TYPE_PROPERTY),
                        TypeField::Indexer { .. } | TypeField::Array { .. } => {}
                    }
                }
            }
            Type::Function(function_type) => {
                for param in &function_type.params {
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
            | Type::Nil(_)
            | Type::True(_)
            | Type::False(_)
            | Type::StringLiteral(_)
            | Type::Number(_)
            | Type::Error(_) => {}
        }
        self.walk_type(ty);
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

const fn stdlib_classify(entry: &StdlibEntry) -> Classification {
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
const fn member_classify(entry: &StdlibEntry) -> Classification {
    let mut classification = stdlib_classify(entry);
    if classification.token_type == TYPE_FUNCTION {
        classification.token_type = TYPE_METHOD;
    }
    classification
}

fn utf16_length(source: &str, span: Span) -> u32 {
    let end = offset_to_index(span.end).min(source.len());
    source.get(offset_to_index(span.start)..end).map_or(0, |text| {
        if text.is_ascii() {
            index_to_offset(text.len())
        } else {
            index_to_offset(text.encode_utf16().count())
        }
    })
}

#[cfg(test)]
mod tests {
    use luck_core::LuaTarget;

    use super::*;
    use crate::fixtures::document;

    fn parse_document(source: &str, target: LuaTarget) -> DocumentState {
        let document = document(source, target);
        assert!(document.parse().errors.is_empty(), "{:?}", document.parse().errors);
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
                "\
Players = Services.Players
local object = {field = Players}
if not object:IsA('Tool') and not object:IsA('HopperBin') then
  if 0 < #getPlayer(arg1[1], arg2) then print(object.field) end
end",
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
            "\
type Alias<T> = {field: module.Member<T>, callback: (arg: T) -> T}
local value: Alias<number> = {}
local item = {field = value}
local chosen = value :: module.Member<number>
",
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
            "\
local value: number = 1
local text: string = ''
local shape: typeof(value) = value
function handler(arg: boolean): number return value end",
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
            "\
local callback: (number) -> number = function(value) return value end
local saved = callback
function object:method<T>(value: T): T return value end
object:method<<number>>(callback<<number>>(1))
",
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
            "\
local print = 1
local alias = print
local math = {floor = function(value) return value end}
math.floor(alias)
",
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
            (
                "\
local limit <const> = 1
print(limit)",
                LuaTarget::Lua54,
            ),
            (
                "\
const limit = 1
print(limit)",
                LuaTarget::Luau,
            ),
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
        let document = parse_document(
            "\
local f = io.open('x')
local n = f:read('n')",
            LuaTarget::Lua54,
        );
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
        let tokens = highlights(&parse_document(
            "\
global function setup()
end
",
            LuaTarget::Lua55,
        ));
        let name = tokens.iter().find(|token| token.text == "setup").expect("setup");
        assert_eq!(name.kind, "function", "{tokens:?}");
    }

    #[test]
    fn a_recovered_assignment_target_emits_no_overlapping_token() {
        // `a.b(c)` can't be assigned, so recovery names it with an empty
        // identifier spanning the whole call. `highlights` asserts that no
        // two tokens overlap and that each has a length.
        let document = document("a.b(c), d = 1\n", LuaTarget::Lua54);
        assert!(!document.parse().errors.is_empty(), "the target must be invalid");
        let tokens = highlights(&document);
        assert!(
            tokens.iter().all(|token| !token.text.contains('(')),
            "a token must cover one identifier: {tokens:?}"
        );
    }
}
