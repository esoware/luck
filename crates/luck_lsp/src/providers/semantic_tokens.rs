//! Identifier highlighting layered over the `TextMate` grammar.
//!
//! One walk classifies each name where the tree holds it, so types and table
//! keys read apart from value expressions that look the same, and a name
//! finds what it declares or refers to by its symbol and reference.

use ls_types::{
    Position, Range, SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokens,
    SemanticTokensLegend, SemanticTokensRangeResult, SemanticTokensResult,
};
use luck_ast::{
    Assignment, ChainHead, Expression, Field, FunctionDecl, FunctionName, FunctionTypeParam,
    GenericPackType, GenericTypeParam, GlobalDeclaration, GlobalFunction, LocalAssignment,
    LocalFunction, Name, NamedType, NumericFor, Parameter, Suffix, SuffixedExpression,
    TableConstructor, Type, TypeDeclaration, TypeDeclarationValue, TypeField, Var, VarArgParam,
    Visit,
};
use luck_semantic::SymbolKind;
use luck_stdlib::{SemanticAnalysis, StdlibEntry};
use luck_token::Span;
use rustc_hash::FxHashMap;

use crate::document::DocumentState;

/// In the order of [`TokenType`], whose discriminant is the index here.
const TOKEN_TYPES: &[SemanticTokenType] = &[
    SemanticTokenType::NAMESPACE,
    SemanticTokenType::FUNCTION,
    SemanticTokenType::PROPERTY,
    SemanticTokenType::VARIABLE,
    SemanticTokenType::PARAMETER,
    SemanticTokenType::METHOD,
    SemanticTokenType::TYPE,
];

/// What a name is, which a token carries as its index in [`TOKEN_TYPES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenType {
    Namespace,
    Function,
    Property,
    Variable,
    Parameter,
    Method,
    Type,
}

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
    token_type: TokenType,
    modifiers: u32,
}

impl Classification {
    const fn plain(token_type: TokenType) -> Self {
        Self { token_type, modifiers: 0 }
    }

    /// A value's name that isn't a function, which reads as read-only where
    /// it's spelled like a constant.
    fn value(token_type: TokenType, name: &str) -> Self {
        let modifiers = if is_screaming_case(name) { MODIFIER_READONLY } else { 0 };
        Self { token_type, modifiers }
    }
}

/// A document's classified names, in source order, none empty and none
/// overlapping another.
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

/// Matches the grammar's all-caps constant pattern: `[A-Z][A-Z0-9_]*`.
fn is_screaming_case(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|rest| rest.is_ascii_uppercase() || rest.is_ascii_digit() || rest == '_')
}

pub(crate) fn classify(document: &DocumentState) -> Highlights {
    let semantic = document.semantic();
    let (tree, stdlib) = (semantic.scoping(), semantic.stdlib());
    let globals = tree
        .names()
        .map(|(_, name)| {
            stdlib
                .globals
                .get(name)
                .map_or(Classification::plain(TokenType::Variable), stdlib_classify)
        })
        .collect();
    let mut classifier = Classifier {
        semantic,
        globals,
        declared: vec![None; tree.symbols().len()],
        shapes: FxHashMap::default(),
        highlights: Vec::new(),
    };
    classifier.visit_chunk(&document.parse().chunk);
    classifier.highlights
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
        let delta_line = line - previous.line;
        let delta_start = if delta_line == 0 { character - previous.character } else { character };
        data.push(SemanticToken {
            delta_line,
            delta_start,
            length: document.length(span),
            token_type: classification.token_type as u32,
            token_modifiers_bitset: classification.modifiers,
        });
        previous = Position { line, character };
    }
    SemanticTokens { result_id: None, data }
}

/// What a name's value is where it's written, which makes a variable a
/// function and a field a method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueShape {
    Called,
    AssignedFunction,
}

/// Where a `Name(.Name)*` path stands before its next member: at its root,
/// or past a member, with the stdlib entry the path reaches there, if any.
#[derive(Clone, Copy)]
enum PathAt<'ast> {
    Root(&'ast Name<'ast>),
    Member(Option<&'ast StdlibEntry<'ast>>),
}

struct Classifier<'document> {
    semantic: &'document SemanticAnalysis<'document>,
    /// Each name's classification as a global, by name index, so a name read
    /// many times is looked up once.
    globals: Vec<Classification>,
    /// Each symbol's classification where it's declared, by symbol index,
    /// which its references take on. A declaration comes before every
    /// reference to it.
    declared: Vec<Option<Classification>>,
    /// The shape of each name, by where it starts, that a call calls or an
    /// assignment gives a function, which the walk meets after the call or
    /// the assignment.
    shapes: FxHashMap<u32, ValueShape>,
    highlights: Highlights,
}

impl<'document> Classifier<'document> {
    /// Keeps the highlights in order and apart, so a name recovery gave
    /// another's span can't break the encoding.
    fn push(&mut self, span: Span, classification: Classification) {
        if !span.is_empty() && self.highlights.last().is_none_or(|(last, _)| last.end <= span.start)
        {
            self.highlights.push((span, classification));
        }
    }

    fn plain(&mut self, name: &Name<'_>, token_type: TokenType) {
        if name.written().is_some() {
            self.push(name.span, Classification::plain(token_type));
        }
    }

    /// A name that declares a local, given the value it's declared with, if
    /// any. A local is a function where its value is one, read-only where
    /// writing it is an error, and read-only too where it's spelled like a
    /// constant and isn't a function.
    fn declaration(&mut self, name: &Name<'_>, value: Option<&Expression<'_>>) {
        let tree = self.semantic.scoping();
        let Some(symbol_id) = tree.symbol_of(name) else {
            return;
        };
        if name.written().is_none_or(|text| text == "self") {
            return;
        }
        let symbol = tree.symbol(symbol_id);
        let mut classification = symbol_classify(symbol.kind);
        if classification.token_type == TokenType::Variable {
            classification = if matches!(value, Some(Expression::FunctionDef(_))) {
                Classification::plain(TokenType::Function)
            } else {
                Classification::value(TokenType::Variable, name.text.as_str())
            };
        }
        if symbol.is_read_only {
            classification.modifiers |= MODIFIER_READONLY;
        }
        self.declared[symbol_id.index()] = Some(classification);
        self.push(name.span, classification);
    }

    /// A name that reads or writes a local or a global. A variable is a
    /// function where it's called or given one, anything called is, and a
    /// local's reference takes on that it's a function, unless it's ever
    /// reassigned, and that it's read-only.
    fn reference(&mut self, name: &Name<'_>) {
        let shape = self.shapes.remove(&name.span.start);
        let tree = self.semantic.scoping();
        let Some(reference) = tree.reference_of(name).map(|id| tree.reference(id)) else {
            return;
        };
        if name.written().is_none_or(|text| text == "self") {
            return;
        }
        let mut classification = match reference.resolved {
            Some(symbol_id) => symbol_classify(tree.symbol(symbol_id).kind),
            None if tree.is_host_global(reference) => self.globals[reference.name.index()],
            None => Classification::plain(TokenType::Variable),
        };
        if classification.token_type == TokenType::Variable {
            classification = if shape.is_some() {
                Classification::plain(TokenType::Function)
            } else {
                Classification::value(TokenType::Variable, name.text.as_str())
            };
        }
        if shape == Some(ValueShape::Called) {
            classification.token_type = TokenType::Function;
        }
        if let Some(symbol_id) = reference.resolved
            && let Some(declared) = self.declared[symbol_id.index()]
        {
            if !tree.symbol(symbol_id).is_modified && declared.token_type == TokenType::Function {
                classification.token_type = TokenType::Function;
            }
            classification.modifiers |= declared.modifiers & MODIFIER_READONLY;
        }
        self.push(name.span, classification);
    }

    /// A field or a method's name, given the stdlib entry its path reaches,
    /// if any. A member that's called, or given a function, is a method.
    fn member(
        &mut self,
        name: &Name<'_>,
        entry: Option<&StdlibEntry<'_>>,
        shape: Option<ValueShape>,
    ) {
        let Some(text) = name.written() else {
            return;
        };
        let mut classification = match entry {
            Some(entry) => member_classify(entry),
            None if shape.is_some() => Classification::plain(TokenType::Method),
            None => Classification::value(TokenType::Property, text),
        };
        if shape == Some(ValueShape::Called) {
            classification.token_type = TokenType::Method;
        }
        self.push(name.span, classification);
    }

    /// The stdlib entry `member` names after `at`, which is `None` past
    /// anything but a name or a field. A root that reaches the host's global
    /// looks up the library, a local of known shape its shape, and any other
    /// root nothing.
    fn step(
        &self,
        at: Option<PathAt<'document>>,
        member: &str,
    ) -> Option<&'document StdlibEntry<'document>> {
        match at? {
            PathAt::Root(root) => {
                let tree = self.semantic.scoping();
                let reference = tree.reference(tree.reference_of(root)?);
                self.semantic.resolve_stdlib_path(&[root.text.as_str(), member], reference)
            }
            PathAt::Member(entry) => self.semantic.stdlib().child(entry?, member),
        }
    }

    /// Marks the name `callee` calls, through parentheses, casts, and Luau
    /// type instantiations.
    fn mark_callee(&mut self, callee: ChainHead<'_>) {
        let mut current = callee;
        loop {
            current = match current.split_last() {
                Some((_, Suffix::Field(name))) => {
                    self.shapes.insert(name.span.start, ValueShape::Called);
                    return;
                }
                Some((before, Suffix::TypeInstantiation { .. })) => before, // Luau
                Some((_, Suffix::Index { .. } | Suffix::Call(_) | Suffix::MethodCall(_))) => {
                    return;
                }
                None => match current.prefix {
                    Expression::Name(name) => {
                        self.shapes.insert(name.span.start, ValueShape::Called);
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

impl<'ast> Visit<'ast> for Classifier<'ast> {
    fn visit_expr(&mut self, expr: &'ast Expression<'ast>) {
        if let Expression::Name(name) = expr {
            self.reference(name);
        } else {
            self.walk_expr(expr);
        }
    }

    fn visit_var(&mut self, var: &'ast Var<'ast>) {
        match var {
            Var::Name(name) => self.reference(name),
            Var::Suffixed(chain) => self.visit_suffixed_expr(chain),
        }
    }

    /// Each member resolves from the one before it, so a path resolves one
    /// member at a time rather than walking back to its root each time.
    fn visit_suffixed_expr(&mut self, suffixed_expr: &'ast SuffixedExpression<'ast>) {
        for (i, suffix) in suffixed_expr.suffixes.iter().enumerate() {
            if matches!(suffix, Suffix::Call(_)) {
                self.mark_callee(suffixed_expr.head(i));
            }
        }
        self.visit_expr(&suffixed_expr.prefix);
        let mut at = if let Expression::Name(root) = &suffixed_expr.prefix {
            Some(PathAt::Root(root))
        } else {
            None
        };
        for suffix in &suffixed_expr.suffixes {
            match suffix {
                Suffix::Field(name) => {
                    let entry = self.step(at, name.text.as_str());
                    at = at.map(|_| PathAt::Member(entry));
                    let shape = self.shapes.remove(&name.span.start);
                    self.member(name, entry, shape);
                }
                Suffix::MethodCall(call) => {
                    let entry = self.step(at, call.name.text.as_str());
                    at = None;
                    self.member(&call.name, entry, Some(ValueShape::Called));
                    if let Some(type_args) = &call.type_args {
                        self.visit_type_args(type_args);
                    }
                    self.visit_function_args(&call.args);
                }
                Suffix::Index { .. } | Suffix::Call(_) | Suffix::TypeInstantiation { .. } => {
                    at = None;
                    self.visit_suffix(suffix);
                }
            }
        }
    }

    fn visit_assignment(&mut self, assignment: &'ast Assignment<'ast>) {
        for (target, value) in assignment.targets.iter().zip(&assignment.values) {
            if !matches!(value, Expression::FunctionDef(_)) {
                continue;
            }
            let assigned = match target {
                Var::Name(name) => Some(&**name),
                Var::Suffixed(chain) => {
                    if let Suffix::Field(name) = chain.suffixes.last() {
                        Some(name)
                    } else {
                        None
                    }
                }
            };
            if let Some(name) = assigned {
                self.shapes.insert(name.span.start, ValueShape::AssignedFunction);
            }
        }
        self.walk_assignment(assignment);
    }

    fn visit_local_assignment(&mut self, local_assignment: &'ast LocalAssignment<'ast>) {
        for (i, attributed) in local_assignment.names.iter().enumerate() {
            self.declaration(&attributed.name, local_assignment.exprs.get(i));
            if let Some(annotation) = &attributed.type_annotation {
                self.visit_type(annotation);
            }
        }
        for expr in &local_assignment.exprs {
            self.visit_expr(expr);
        }
    }

    // Lua 5.5
    fn visit_global_declaration(&mut self, global_declaration: &'ast GlobalDeclaration<'ast>) {
        let leading = global_declaration.attribute.as_deref();
        for attributed in &global_declaration.names {
            if let Some(text) = attributed.name.written() {
                let mut classification = Classification::value(TokenType::Variable, text);
                if attributed
                    .attribute_or(leading)
                    .is_some_and(|attribute| attribute.name.text == "const")
                {
                    classification.modifiers |= MODIFIER_READONLY;
                }
                self.push(attributed.name.span, classification);
            }
        }
        for expr in &global_declaration.exprs {
            self.visit_expr(expr);
        }
    }

    fn visit_local_function(&mut self, local_function: &'ast LocalFunction<'ast>) {
        for attribute in &local_function.attributes {
            self.visit_function_attribute(attribute);
        }
        self.declaration(&local_function.name, None);
        self.visit_function_body(&local_function.body);
    }

    // Lua 5.5
    fn visit_global_function(&mut self, global_function: &'ast GlobalFunction<'ast>) {
        self.plain(&global_function.name, TokenType::Function);
        self.visit_function_body(&global_function.body);
    }

    /// `function f()` declares a function, and `function a.b.c()` and
    /// `function a.b:c()` a method on the table `a.b` reads.
    fn visit_function_decl(&mut self, function_decl: &'ast FunctionDecl<'ast>) {
        for attribute in &function_decl.attributes {
            self.visit_function_attribute(attribute);
        }
        let FunctionName { names, method, .. } = &function_decl.name;
        let is_method = method.is_some() || names.len() > 1;
        let mut at = None;
        for (i, name) in names.iter().enumerate() {
            if i == 0 {
                if !is_method {
                    self.shapes.insert(name.span.start, ValueShape::Called);
                }
                self.reference(name);
                at = Some(PathAt::Root(name));
            } else if method.is_none() && i + 1 == names.len() {
                self.plain(name, TokenType::Method);
            } else {
                let entry = self.step(at, name.text.as_str());
                at = at.map(|_| PathAt::Member(entry));
                self.member(name, entry, None);
            }
        }
        if let Some(method_name) = method {
            self.plain(method_name, TokenType::Method);
        }
        self.visit_function_body(&function_decl.body);
    }

    fn visit_param(&mut self, param: &'ast Parameter<'ast>) {
        self.declaration(&param.name, None);
        if let Some(annotation) = &param.type_annotation {
            self.visit_type(annotation);
        }
    }

    // Lua 5.5 names a vararg, and Luau types one.
    fn visit_var_arg_param(&mut self, var_arg_param: &'ast VarArgParam<'ast>) {
        if let Some(name) = &var_arg_param.name {
            self.declaration(name, None);
        }
        if let Some(annotation) = &var_arg_param.type_annotation {
            self.visit_type(annotation);
        }
    }

    fn visit_numeric_for(&mut self, numeric_for: &'ast NumericFor<'ast>) {
        self.declaration(&numeric_for.name, None);
        if let Some(annotation) = &numeric_for.type_annotation {
            self.visit_type(annotation);
        }
        self.visit_expr(&numeric_for.start);
        self.visit_expr(&numeric_for.limit);
        if let Some(step) = &numeric_for.step {
            self.visit_expr(step);
        }
        self.visit_block(&numeric_for.block);
    }

    fn visit_table_constructor(&mut self, table_constructor: &'ast TableConstructor<'ast>) {
        for field in &table_constructor.fields {
            match field {
                Field::Named { name, value } => {
                    let is_method = matches!(value, Expression::FunctionDef(_));
                    self.plain(
                        name,
                        if is_method { TokenType::Method } else { TokenType::Property },
                    );
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

    // Luau
    fn visit_type_declaration(&mut self, type_declaration: &'ast TypeDeclaration<'ast>) {
        let token_type = match &type_declaration.value {
            TypeDeclarationValue::Alias(_) => TokenType::Type,
            TypeDeclarationValue::TypeFunction(_) => TokenType::Function,
        };
        self.plain(&type_declaration.name, token_type);
        self.walk_type_declaration(type_declaration);
    }

    // Luau
    fn visit_generic_type_param(&mut self, generic_type_param: &'ast GenericTypeParam<'ast>) {
        self.plain(&generic_type_param.name, TokenType::Type);
        self.walk_generic_type_param(generic_type_param);
    }

    // Luau: a primitive keeps its grammar scope.
    fn visit_type(&mut self, ty: &'ast Type<'ast>) {
        if let Type::Name(name) = ty {
            if !matches!(
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
                self.plain(name, TokenType::Type);
            }
        } else {
            self.walk_type(ty);
        }
    }

    // Luau
    fn visit_named_type(&mut self, named_type: &'ast NamedType<'ast>) {
        if let Some(prefix) = &named_type.prefix {
            self.plain(prefix, TokenType::Variable);
        }
        self.plain(&named_type.name, TokenType::Type);
        self.walk_named_type(named_type);
    }

    // Luau
    fn visit_generic_pack_type(&mut self, generic_pack_type: &'ast GenericPackType<'ast>) {
        self.plain(&generic_pack_type.name, TokenType::Type);
    }

    // Luau
    fn visit_type_field(&mut self, type_field: &'ast TypeField<'ast>) {
        if let TypeField::Named { name, .. } = type_field {
            self.plain(name, TokenType::Property);
        }
        self.walk_type_field(type_field);
    }

    // Luau
    fn visit_function_type_param(&mut self, function_type_param: &'ast FunctionTypeParam<'ast>) {
        if let Some(name) = &function_type_param.name {
            self.plain(name, TokenType::Parameter);
        }
        self.walk_function_type_param(function_type_param);
    }
}

const fn symbol_classify(kind: SymbolKind) -> Classification {
    match kind {
        SymbolKind::Parameter | SymbolKind::ImplicitSelf | SymbolKind::ImplicitArg => {
            Classification::plain(TokenType::Parameter)
        }
        SymbolKind::FunctionName => Classification::plain(TokenType::Function),
        SymbolKind::Local | SymbolKind::IteratorVariable | SymbolKind::NumericForVariable => {
            Classification::plain(TokenType::Variable)
        }
    }
}

const fn stdlib_classify(entry: &StdlibEntry<'_>) -> Classification {
    let token_type = match entry {
        StdlibEntry::Function(_) => TokenType::Function,
        StdlibEntry::Namespace(_) => TokenType::Namespace,
        StdlibEntry::Constant(_) | StdlibEntry::Property(_) => TokenType::Property,
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
const fn member_classify(entry: &StdlibEntry<'_>) -> Classification {
    let mut classification = stdlib_classify(entry);
    if matches!(classification.token_type, TokenType::Function) {
        classification.token_type = TokenType::Method;
    }
    classification
}

#[cfg(test)]
mod tests {
    use luck_token::{LuaTarget, index_to_offset};

    use super::*;
    use crate::document::span_text;
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
                position.line += token.delta_line;
                position.character = if token.delta_line == 0 {
                    position.character + token.delta_start
                } else {
                    token.delta_start
                };
                assert!(position >= previous_end, "overlapping tokens: {data:?}");
                assert!(token.length > 0, "zero-length token: {data:?}");
                previous_end = Position::new(position.line, position.character + token.length);
                let span = Span::new(document.offset(position), document.offset(previous_end));
                Highlight {
                    text: span_text(document.text(), span).to_owned(),
                    kind: legend.token_types[token.token_type as usize].as_str().to_owned(),
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
    fn a_binding_that_can_t_be_written_is_readonly_everywhere() {
        for (source, target, name) in [
            ("const function f() end f()", LuaTarget::Luau, "f"),
            ("for i = 1, 2 do print(i) end", LuaTarget::Lua55, "i"),
            ("local <const> a, b = 1, 2 print(a, b)", LuaTarget::Lua55, "b"),
            ("global <const> limit = 1", LuaTarget::Lua55, "limit"),
        ] {
            let tokens = highlights(&parse_document(source, target));
            let named: Vec<&Highlight> = tokens.iter().filter(|token| token.text == name).collect();
            assert!(!named.is_empty(), "{source}: {tokens:?}");
            for token in named {
                assert!(token.modifiers.iter().any(|modifier| modifier == "readonly"), "{source}");
            }
        }
    }

    #[test]
    fn a_member_called_or_given_a_function_is_a_method() {
        let tokens = highlights(&parse_document(
            "\
t.run = function() end
t.go(t.x, (t.cast :: any)(), t.later)
",
            LuaTarget::Luau,
        ));
        let kinds: Vec<(&str, &str)> =
            tokens.iter().map(|token| (token.text.as_str(), token.kind.as_str())).collect();
        assert_eq!(
            kinds,
            [
                ("t", "variable"),
                ("run", "method"),
                ("t", "variable"),
                ("go", "method"),
                ("t", "variable"),
                ("x", "property"),
                ("t", "variable"),
                ("cast", "method"),
                ("t", "variable"),
                ("later", "property"),
            ]
        );
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
